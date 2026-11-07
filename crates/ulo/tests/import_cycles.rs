//! Modules importing one another (sections 8.2, 8.6, 9.2). An import is a visibility fact, so a
//! cycle of imports wires, connects and serves: a binding reads an export across the cycle in
//! either direction, a re-export passes one on, each member contributes once, hooks keep the order
//! the bindings give them, a lazy load brings a cycle of its own into a graph that holds one, and a
//! module inside a cycle reads its qualifier by the rule a module two importers share follows.
//!
//! A module importing itself is refused; `wiring_modules.rs` pins that refusal.

#[macro_use]
mod common;

use std::sync::{Arc, Mutex};

use common::*;
use ulo::{App, Connected, Dep, ExecOptions, Execution, HandlerSpec, Keyed, Many, Module, MountedHandler, Signal, WiringError};

struct R;
struct A;
struct B;
struct C;
struct L1;
struct L2;

struct Left(&'static str);
struct Right(&'static str);
struct AReads(String);
struct BReads(String);

controller!(InB, ulo::scope::Auto, [reads: Dep<BReads>], |m| m.handler::<TA, _>(HandlerSpec::new("h", ())));

/// The one handler mounted on `TA`.
fn handler(app: &App<Connected>) -> MountedHandler<TA> {
    let mut handlers = app.handle().mounted::<TA>().expect("the app has a runtime");
    assert_eq!(handlers.len(), 1, "one handler was mounted");
    handlers.remove(0)
}

/// One call through `handler`, as its transport would run it, answering what reading `T` in the
/// handler's module gives, as `text` renders it.
async fn served<T: Send + Sync + 'static>(handler: &MountedHandler<TA>, text: fn(&T) -> String) -> String {
    let exec = Execution::open(handler.module(), ExecOptions::new()).expect("the app is running");
    let calls = exec.handle();
    let answer = Mutex::new(String::new());
    ulo::dispatch(handler, &calls, &(), |_cx| async {
        let read = calls.get::<T>().await?;
        *answer.lock().unwrap() = text(&read);
        Ok(())
    })
    .await
    .expect("the call answered");
    answer.into_inner().unwrap()
}

/// `Mod<A>` of the two-module cycle: exports `Left` and reads `Right` from `Mod<B>`, which it
/// imports.
fn pair_a() -> Mod<A> {
    Mod::new(|m| {
        m.import(pair_b());
        m.value(Left("left"));
        m.export::<Left>();
        m.singleton(|right: Dep<Right>| {
            let seen = format!("a sees {}", right.0);
            async move { AReads(seen) }
        });
        m.export::<AReads>();
    })
}

/// `Mod<B>`: imports `Mod<A>` back, which closes the cycle, reads `Left` through that import and
/// serves it through a controller.
fn pair_b() -> Mod<B> {
    Mod::new(|m| {
        m.import(pair_a());
        m.value(Right("right"));
        m.export::<Right>();
        m.singleton(|left: Dep<Left>| {
            let seen = format!("b sees {}", left.0);
            async move { BReads(seen) }
        });
        m.controller::<InB>();
    })
}

/// Planted break: `enter` refusing every import that closes a cycle, as a self-import is.
#[tokio::test]
async fn two_modules_importing_each_other_wire_connect_and_serve() {
    let app = connected_on_tokio(Mod::<R>::new(|m| m.import(pair_a()))).await;

    let a = app.get::<AReads>().await.expect("the root reads Mod<A>'s export");
    assert_eq!(a.0, "a sees right", "Mod<A> reads what the module it imports exports");
    let handler = handler(&app);
    assert_eq!(handler.module().name().to_string(), mod_name::<B>());
    assert_eq!(
        served::<BReads>(&handler, |r| r.0.clone()).await,
        "b sees left",
        "Mod<B> reads, through the import closing the cycle, what Mod<A> exports"
    );
    drop(handler);
    close(app).await;
}

struct Va;
struct Vb;
struct Vc;
struct Ra(&'static str);
struct Rb(&'static str);
struct Rc(&'static str);

controller!(InC, ulo::scope::Auto, [reads: Dep<Rc>], |m| m.handler::<TA, _>(HandlerSpec::new("h", ())));

fn ring_a() -> Mod<A> {
    Mod::new(|m| {
        m.import(ring_b());
        m.value(Va);
        m.export::<Va>();
        m.singleton(|_b: Dep<Vb>| async { Ra("a read b") });
        m.export::<Ra>();
    })
}

fn ring_b() -> Mod<B> {
    Mod::new(|m| {
        m.import(ring_c());
        m.value(Vb);
        m.export::<Vb>();
        m.singleton(|_c: Dep<Vc>| async { Rb("b read c") });
        m.export::<Rb>();
    })
}

/// Closes `A -> B -> C -> A`.
fn ring_c() -> Mod<C> {
    Mod::new(|m| {
        m.import(ring_a());
        m.value(Vc);
        m.export::<Vc>();
        m.singleton(|_a: Dep<Va>| async { Rc("c read a") });
        m.controller::<InC>();
    })
}

/// Planted break: `enter` refusing every import that closes a cycle.
#[tokio::test]
async fn three_modules_in_a_cycle_wire_connect_and_serve() {
    let app = connected_on_tokio(Mod::<R>::new(|m| m.import(ring_a()))).await;

    assert_eq!(app.get::<Ra>().await.expect("the root reads Mod<A>'s export").0, "a read b");
    let b = app.module::<Mod<B>>().expect("Mod<B> is in the graph");
    assert_eq!(b.get::<Rb>().await.expect("Mod<B> reads its own binding").0, "b read c");
    let handler = handler(&app);
    assert_eq!(
        served::<Rc>(&handler, |r| r.0.to_owned()).await,
        "c read a",
        "Mod<C> reads, through the import closing the cycle, what Mod<A> exports"
    );
    drop(handler);
    close(app).await;
}

struct Early;
struct Late;

fn hooked_a(log: Log) -> Mod<A> {
    Mod::new(move |m| {
        m.import(hooked_b(log.clone()));
        m.singleton(|| async { Early }).on_init(init(&log, "a init")).on_destroy(destroy(&log, "a destroy"));
        m.export::<Early>();
        m.on_init(init(&log, "a module init"));
        m.on_destroy(destroy(&log, "a module destroy"));
    })
}

/// Placed before `Mod<A>` in collection order, its singleton reading `Mod<A>`'s through the import
/// closing the cycle.
fn hooked_b(log: Log) -> Mod<B> {
    Mod::new(move |m| {
        m.import(hooked_a(log.clone()));
        m.singleton(|_early: Dep<Early>| async { Late }).on_init(init(&log, "b init")).on_destroy(destroy(&log, "b destroy"));
        m.on_init(init(&log, "b module init"));
        m.on_destroy(destroy(&log, "b module destroy"));
    })
}

fn init(log: &Log, text: &'static str) -> impl Fn() -> std::future::Ready<Result<(), Boom>> + Send + Sync + 'static {
    let log = log.clone();
    move || {
        log.push(text);
        std::future::ready(Ok(()))
    }
}

fn destroy(log: &Log, text: &'static str) -> impl Fn() -> std::future::Ready<()> + Send + Sync + 'static {
    let log = log.clone();
    move || std::future::ready(log.push(text))
}

/// Collection order puts `Mod<B>` first, since the walk reaches it inside `Mod<A>`; the binding
/// `Mod<B>` reads from `Mod<A>` puts `Mod<A>`'s hooks first. Planted break: `connect_order`
/// ignoring dependencies, which runs `Mod<B>`'s first.
#[tokio::test]
async fn hooks_in_a_cycle_run_in_dependency_order_and_shut_down_in_reverse() {
    let log = Log::new();
    let l = log.clone();
    let app = connects(Mod::<R>::new(move |m| m.import(hooked_a(l.clone())))).await;
    assert_eq!(
        log.events(),
        ["a init", "a module init", "b init", "b module init"],
        "the singleton read across the cycle starts first, each module's own hook after its provider's"
    );

    let startup = log.events().len();
    app.close(Signal::new("done")).await.expect("shutdown reported a failure");
    assert_eq!(
        log.events()[startup..],
        ["b module destroy", "b destroy", "a module destroy", "a destroy"],
        "shutdown is the exact reverse"
    );
}

struct VL2;
struct RL1;
struct RL2(&'static str);

fn lazy_l1(log: Log) -> Mod<L1> {
    Mod::new(move |m| {
        m.import(lazy_l2(log.clone()));
        m.singleton(|_v: Dep<VL2>| async { RL1 }).on_init(init(&log, "l1 init"));
        m.export::<RL1>();
    })
}

/// Closes the lazy modules' cycle, and imports a member of the base graph's.
fn lazy_l2(log: Log) -> Mod<L2> {
    Mod::new(move |m| {
        m.import(lazy_l1(log.clone()));
        m.import(Mod::<B>::bare());
        m.value(VL2);
        m.export::<VL2>();
        m.singleton(|right: Dep<Right>, _l1: Dep<RL1>| {
            let read = right.0;
            async move { RL2(read) }
        })
        .on_init(init(&log, "l2 init"));
    })
}

/// A base graph holding a cycle takes a load whose modules form one of their own and import a
/// member of the base's. Planted break: `enter` refusing every import that closes a cycle.
#[tokio::test]
async fn a_lazy_load_with_a_cycle_of_its_own_joins_a_cyclic_graph() {
    let app = connects(Mod::<R>::new(|m| m.import(pair_a()))).await;
    let handle = app.handle();
    let log = Log::new();

    let loaded = handle.load(lazy_l1(log.clone())).await.unwrap_or_else(|error| panic!("load failed: {error}"));
    assert_eq!(loaded.name().to_string(), mod_name::<L1>());
    assert_eq!(log.events(), ["l1 init", "l2 init"], "the load starts its singletons in dependency order");
    let l2 = handle.module::<Mod<L2>>().expect("the load brought Mod<L2>");
    assert_eq!(
        l2.get::<RL2>().await.expect("Mod<L2> reads its own binding").0,
        "right",
        "Mod<L2> reads what the base graph's Mod<B> exports"
    );
    assert!(loaded.get::<RL1>().await.is_ok(), "Mod<L1> reads its own binding");
    close(app).await;
}

struct KInner;
struct Member;
struct Primary;

/// What each registration of `Mod<Member>` read from `ModuleDef::qualifier`.
type Seen = Arc<Mutex<Vec<Option<String>>>>;

fn member(seen: Seen, back: bool) -> Mod<Member> {
    Mod::new(move |m| {
        seen.lock().unwrap().push(m.qualifier().map(|q| q.to_string()));
        if back {
            m.import(keyed(seen.clone(), back));
        }
    })
}

/// `Mod<KInner>` keyed `Primary`, importing `Mod<Member>`, which imports it back when `back`.
fn keyed(seen: Seen, back: bool) -> Keyed<Primary, Mod<KInner>> {
    Mod::<KInner>::new(move |m| m.import(member(seen.clone(), back))).keyed::<Primary>()
}

/// The qualifier `Mod<Member>` reads with the root importing the keyed module and the member in
/// the order given, `Mod<Member>` importing the keyed module back when `back`.
fn qualifier_read(keyed_first: bool, back: bool) -> Option<String> {
    let seen = Seen::default();
    let s = seen.clone();
    wires(Mod::<R>::new(move |m| {
        if keyed_first {
            m.import(keyed(s.clone(), back));
            m.import(member(s.clone(), back));
        } else {
            m.import(member(s.clone(), back));
            m.import(keyed(s.clone(), back));
        }
    }));
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "an equal identity registers once");
    seen.into_iter().next().flatten()
}

/// A member of a cycle with a keyed module is a module two importers share, the root and the keyed
/// module, and reads the key of the path that reached it first, in both orders exactly as the
/// acyclic diamond of the same imports does. Planted break: `inherited_keyed` left `None`.
#[test]
fn a_module_in_a_cycle_reads_its_qualifier_as_a_shared_import_does() {
    for back in [true, false] {
        let shape = if back { "the cycle" } else { "the diamond" };
        assert_eq!(
            qualifier_read(true, back).as_deref(),
            Some("Primary"),
            "{shape}: reached first through the keyed module, the member reads its key"
        );
        assert_eq!(qualifier_read(false, back), None, "{shape}: reached first from the root, the member reads no key");
    }
}

struct Shared(u32);
struct Unbound;

/// `Mod<A>` binds and exports `Shared`; `Mod<B>`, which `Mod<A>` imports and which imports it
/// back, re-exports it.
fn reexporting_a() -> Mod<A> {
    Mod::new(|m| {
        m.import(reexporting_b());
        m.value(Shared(7));
        m.export::<Shared>();
    })
}

fn reexporting_b() -> Mod<B> {
    Mod::new(|m| {
        m.import(reexporting_a());
        m.reexport::<Shared>();
    })
}

/// A re-export resolves against its module's view, so one of a key the other member of a cycle
/// exports reaches a module importing only the re-exporting member. Planted break: `enter`
/// refusing every import that closes a cycle.
#[tokio::test]
async fn a_reexport_inside_a_cycle_passes_the_other_member_s_export_on() {
    let app = connects(Mod::<R>::new(|m| m.import(reexporting_b()))).await;
    assert_eq!(app.get::<Shared>().await.expect("the root sees Mod<B>'s re-export").0, 7);
    close(app).await;
}

/// Two members re-exporting a key neither binds resolve nothing in any round: each is refused,
/// and the rounds end.
#[test]
fn two_members_re_exporting_a_key_neither_binds_are_each_refused() {
    fn a() -> Mod<A> {
        Mod::new(|m| {
            m.import(b());
            m.reexport::<Unbound>();
        })
    }
    fn b() -> Mod<B> {
        Mod::new(|m| {
            m.import(a());
            m.reexport::<Unbound>();
        })
    }
    let errors = wire_err(Mod::<R>::new(|m| m.import(a())));
    let mut modules = entries!(errors, WiringError::ReexportNotVisible { module, .. } => module.to_string());
    modules.sort();
    let mut expected = vec![mod_name::<A>(), mod_name::<B>()];
    expected.sort();
    assert_eq!(modules, expected);
    assert_eq!(errors.len(), 2, "the two re-exports and nothing else:\n{errors}");
}

trait Named: Send + Sync {
    fn name(&self) -> &'static str;
}

struct Name(&'static str);

impl Named for Name {
    fn name(&self) -> &'static str {
        self.0
    }
}

fn contributing_a() -> Mod<A> {
    Mod::new(|m| {
        m.import(contributing_b());
        m.contribute::<dyn Named>().value(Arc::new(Name("a")));
    })
}

fn contributing_b() -> Mod<B> {
    Mod::new(|m| {
        m.import(contributing_a());
        m.contribute::<dyn Named>().value(Arc::new(Name("b")));
    })
}

/// The walk reaches `Mod<B>` inside `Mod<A>` and does not follow `Mod<B>`'s import of `Mod<A>`
/// back, so each module contributes once and `Mod<B>`'s contributions come first, though it
/// imports `Mod<A>`. Planted break: collection order placing a module when the walk reaches it
/// rather than when it leaves it.
#[tokio::test]
async fn each_member_of_a_cycle_contributes_once_in_collection_order() {
    let app = connects(Mod::<R>::new(|m| {
        m.import(contributing_a());
        m.contribute::<dyn Named>().value(Arc::new(Name("root")));
    }))
    .await;
    let named: Many<dyn Named> = read_many(&app).await.expect("the root reads the collection");
    assert_eq!(named.iter().map(|n| n.name()).collect::<Vec<_>>(), ["b", "a", "root"]);
    close(app).await;
}
